//! Phase 0 spike for dagrs 0.8.1 API assumptions.

use std::sync::Arc;

use async_trait::async_trait;
use dagrs::{
    Action, CompletionStatus, DefaultNode, EnvVar, Graph, InChannels, Node, NodeTable, OutChannels,
    Output,
    event::{GraphEvent, TerminationStatus},
};

struct NoopAction;

#[async_trait]
impl Action for NoopAction {
    async fn run(
        &self,
        _in_channels: &mut InChannels,
        _out_channels: &mut OutChannels,
        _env: Arc<EnvVar>,
    ) -> Output {
        Output::empty()
    }
}

#[tokio::test]
async fn dagrs_081_graph_build_report_and_termination_event_contract() {
    let mut node_table = NodeTable::new();
    let first = DefaultNode::with_action("first".to_string(), NoopAction, &mut node_table);
    let first_id = first.id();
    let second = DefaultNode::with_action("second".to_string(), NoopAction, &mut node_table);
    let second_id = second.id();

    let mut graph = Graph::new();
    let added_first = graph.add_node(first).expect("add first node");
    let added_second = graph.add_node(second).expect("add second node");
    assert_eq!(added_first, first_id);
    assert_eq!(added_second, second_id);
    graph.add_edge(first_id, [second_id]).expect("add edge");

    let mut events = graph.subscribe();
    let report = graph.async_start().await.expect("dagrs async_start report");
    assert_eq!(report.status, CompletionStatus::Succeeded);
    assert_eq!(report.node_total, 2);
    assert_eq!(report.node_succeeded, 2);

    let mut terminated = None;
    for _ in 0..16 {
        match tokio::time::timeout(std::time::Duration::from_secs(1), events.recv()).await {
            Ok(Ok(GraphEvent::ExecutionTerminated { status, error })) => {
                terminated = Some((status, error));
                break;
            }
            Ok(Ok(_)) => continue,
            other => panic!("missing termination event: {other:?}"),
        }
    }

    let (status, error) = terminated.expect("execution terminated event");
    assert_eq!(status, TerminationStatus::Succeeded);
    assert!(error.is_none());
}
