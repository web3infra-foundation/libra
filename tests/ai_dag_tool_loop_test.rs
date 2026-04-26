//! L1 unit tests for the DAG-based tool loop using a scripted (mock) completion model.
//!
//! Exercises `ToolLoopAction` end to end inside a `dagrs` graph by feeding scripted
//! provider responses (`tool_call` followed by `text`) and asserting the requested
//! tool actually executes against a real temp-dir filesystem.
//!
//! **Layer:** L1 — deterministic, no external dependencies. Uses `tempfile::TempDir`
//! for filesystem isolation; tool effects are scoped to that directory.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use dagrs::{
    Action, Content, DefaultNode, EnvVar, Graph, InChannels, Node, NodeTable, OutChannels, Output,
};
use libra::internal::ai::{
    ToolLoopAction,
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
        Function, Text, ToolCall,
    },
    tools::{ToolRegistryBuilder, handlers::ApplyPatchHandler},
};
use tempfile::TempDir;

/// `CompletionModel` impl that returns a queue of pre-baked responses in order.
///
/// Lets the test author script "first the model emits a tool call, then it emits a
/// final text response" without spinning up a real provider. The internal queue is
/// `Arc<Mutex<...>>` so `ScriptedModel` can satisfy the `Clone` bound that
/// `ToolLoopAction` requires while still preserving consumption order across clones.
#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<VecDeque<CompletionResponse<()>>>>,
}

impl ScriptedModel {
    /// Build a `ScriptedModel` that will yield `responses` in order on successive
    /// `completion` calls and then return an error once exhausted.
    fn new(responses: Vec<CompletionResponse<()>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::from(responses))),
        }
    }
}

impl CompletionModel for ScriptedModel {
    type Response = ();

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        self.responses.lock().unwrap().pop_front().ok_or_else(|| {
            CompletionError::ResponseError("No scripted responses remaining".to_string())
        })
    }
}

/// Trivial source action that broadcasts a fixed prompt to the downstream tool-loop
/// node. Mirrors the input node used in `ai_agent_test.rs` so the two test files share
/// a recognisable shape.
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

/// Scenario: drive the full `ToolLoopAction` cycle end to end with a scripted model.
/// The first scripted response is an `apply_patch` tool call against a target file in
/// a temp dir; the second is a plain "done" text. The test asserts (a) the patch was
/// applied — verifying the tool registry, working-dir wiring, and patch dispatch all
/// work — and (b) the final DAG output equals "done", proving the tool loop terminated
/// cleanly on the text response. Acts as a regression guard for the tool-call→tool-
/// result→text-completion sequence.
#[tokio::test]
async fn test_dag_tool_loop_action_applies_patch() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("target.txt");
    std::fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

    let patch = "*** Begin Patch
*** Update File: target.txt
@@
 line 1
-line 2
+line two
 line 3
*** End Patch";

    let scripted = ScriptedModel::new(vec![
        CompletionResponse {
            content: vec![AssistantContent::ToolCall(ToolCall {
                id: "call-1".to_string(),
                name: "apply_patch".to_string(),
                function: Function {
                    name: "apply_patch".to_string(),
                    arguments: serde_json::json!({
                        "input": patch
                    }),
                },
            })],
            reasoning_content: None,
            raw_response: (),
        },
        CompletionResponse {
            content: vec![AssistantContent::Text(Text {
                text: "done".to_string(),
            })],
            reasoning_content: None,
            raw_response: (),
        },
    ]);

    let registry = ToolRegistryBuilder::with_working_dir(temp_dir.path().to_path_buf())
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .build();

    let action = ToolLoopAction::new(scripted, registry, None, Some(0.0));

    let mut node_table = NodeTable::new();
    let a = DefaultNode::with_action(
        "input".to_string(),
        InputGenerator {
            prompt: "Please update the file".to_string(),
        },
        &mut node_table,
    );
    let a_id = a.id();
    let b = DefaultNode::with_action("ai".to_string(), action, &mut node_table);
    let b_id = b.id();

    let mut graph = Graph::new();
    graph.add_node(a).expect("add input node");
    graph.add_node(b).expect("add ai node");
    graph.add_edge(a_id, vec![b_id]).expect("add edge");

    let result = graph.async_start().await;
    assert!(result.is_ok(), "Graph execution failed: {:?}", result.err());

    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("line two"));

    let outputs = graph.get_results::<String>();
    let output = outputs.get(&b_id).unwrap().clone().unwrap();
    assert_eq!(&*output, "done");
}
