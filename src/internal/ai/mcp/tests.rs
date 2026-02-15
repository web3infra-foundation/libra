use std::sync::Arc;

use tempfile::tempdir;
use uuid::Uuid;

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
    let repo_id = Uuid::new_v4();
    let server = LibraMcpServer::new(Some(history_manager), None, Some(storage), repo_id);

    let params = CreateTaskParams {
        title: "Test Task".to_string(),
        description: Some("Description".to_string()),
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
        requested_by_kind: None,
        requested_by_id: None,
        dependencies: None,
        status: None,
        actor_kind: None,
        actor_id: None,
    };

    let actor = server.default_actor().unwrap();
    let result = server.create_task_impl(params, actor).await;
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
