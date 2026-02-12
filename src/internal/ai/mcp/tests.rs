use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use tempfile::tempdir;

use crate::{
    internal::ai::{
        history::HistoryManager,
        mcp::{server::LibraMcpServer, tools::CreateTaskParams},
    },
    utils::storage::local::LocalStorage,
};

#[tokio::test]
async fn test_create_task_tool() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
    ));
    let server = LibraMcpServer::new(Some(history_manager), Some(storage));

    let params = CreateTaskParams {
        title: "Test Task".to_string(),
        description: Some("Description".to_string()),
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
    };

    let result = server.create_task(Parameters(params)).await;
    assert!(result.is_ok());

    let call_result = result.unwrap();
    assert!(!call_result.content.is_empty());
    let content = &call_result.content[0];

    use rmcp::model::RawContent;
    match &content.raw {
        RawContent::Text(text_content) => {
            assert!(text_content.text.contains("Task created with ID"));
        }
        _ => panic!("Expected text content"),
    }
}
