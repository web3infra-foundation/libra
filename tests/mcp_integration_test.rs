use std::sync::Arc;

use libra::{
    internal::ai::{
        history::HistoryManager,
        mcp::{
            server::LibraMcpServer,
            tools::{CreateTaskParams, ListTasksParams},
        },
    },
    utils::storage::local::LocalStorage,
};
use rmcp::{ServerHandler, handler::server::wrapper::Parameters};
use tempfile::tempdir;
#[tokio::test]
async fn test_mcp_integration_server_info() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
    ));
    let server = LibraMcpServer::new(Some(history_manager), Some(storage));

    let info = ServerHandler::get_info(&server);
    assert_eq!(info.server_info.name, "libra");
}

#[tokio::test]
async fn test_mcp_integration_list_resources() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
    ));
    let server = LibraMcpServer::new(Some(history_manager), Some(storage));

    // Call implementation directly to avoid RequestContext
    let resources = server.list_resources_impl().await.unwrap();
    assert!(!resources.is_empty());
    assert!(resources.iter().any(|r| r.uri == "libra://history/latest"));
}

#[tokio::test]
async fn test_mcp_integration_create_and_read_task() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
    ));
    let server = LibraMcpServer::new(Some(history_manager), Some(storage));

    // 1. Create Task
    let params = CreateTaskParams {
        title: "Integration Test Task".to_string(),
        description: Some("Description".to_string()),
        goal_type: Some("feature".to_string()),
        constraints: Some(vec!["Must use Rust".to_string()]),
        acceptance_criteria: None,
    };

    let result = server.create_task(Parameters(params)).await.unwrap();
    let content = &result.content[0];

    // Use serde_json to inspect content to avoid enum variant issues
    let val = serde_json::to_value(content).unwrap();
    let text = val
        .get("text")
        .expect("text field")
        .as_str()
        .expect("string");

    assert!(text.contains("Task created with ID"));

    // Extract ID (simple parsing)
    let id_str = text.split("ID: ").nth(1).unwrap().trim();

    // 2. Read Resource by ID
    let uri = format!("libra://object/{}", id_str);
    let contents = server.read_resource_impl(&uri).await.unwrap();
    assert_eq!(contents.len(), 1);

    let res_val = serde_json::to_value(&contents[0]).unwrap();
    let res_text = res_val
        .get("text")
        .expect("text field")
        .as_str()
        .expect("string");

    println!("Resource content: {}", res_text);
    assert!(res_text.contains("Integration Test Task"));
    assert!(res_text.contains("Must use Rust"));

    // 3. List Tasks
    let list_params = ListTasksParams {
        limit: None,
        status: None,
    };
    let list_result = server.list_tasks(Parameters(list_params)).await.unwrap();

    let list_val = serde_json::to_value(&list_result.content[0]).unwrap();
    let list_text = list_val
        .get("text")
        .expect("text field")
        .as_str()
        .expect("string");

    assert!(list_text.contains(id_str));
    assert!(list_text.contains("Integration Test Task"));
}
