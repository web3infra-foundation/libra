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

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<VecDeque<CompletionResponse<()>>>>,
}

impl ScriptedModel {
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

#[test]
fn test_dag_tool_loop_action_applies_patch() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("target.txt");
    std::fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

    let patch = "@@ -1,3 +1,3 @@
 line 1
-line 2
+line two
 line 3";

    let scripted = ScriptedModel::new(vec![
        CompletionResponse {
            content: vec![AssistantContent::ToolCall(ToolCall {
                id: "call-1".to_string(),
                name: "apply_patch".to_string(),
                function: Function {
                    name: "apply_patch".to_string(),
                    arguments: serde_json::json!({
                        "file_path": file_path,
                        "patch": patch
                    }),
                },
            })],
            raw_response: (),
        },
        CompletionResponse {
            content: vec![AssistantContent::Text(Text {
                text: "done".to_string(),
            })],
            raw_response: (),
        },
    ]);

    let registry = ToolRegistryBuilder::with_working_dir(temp_dir.path().to_path_buf())
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .build();

    let action = ToolLoopAction::new(scripted, registry, None, Some(0.0), 4);

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
    graph.add_node(a);
    graph.add_node(b);
    graph.add_edge(a_id, vec![b_id]);

    let result = graph.start();
    assert!(result.is_ok(), "Graph execution failed: {:?}", result.err());

    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("line two"));

    let outputs = graph.get_results::<String>();
    let output = outputs.get(&b_id).unwrap().clone().unwrap();
    assert_eq!(&*output, "done");
}
