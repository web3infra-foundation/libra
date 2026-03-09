use std::sync::Arc;

use sea_orm::{ConnectionTrait, Database, Schema};
use tempfile::tempdir;

// use uuid::Uuid;
use crate::{
    internal::{
        ai::{
            history::HistoryManager,
            mcp::{resource::CreateTaskParams, server::LibraMcpServer},
        },
        model::reference,
    },
    utils::storage::local::LocalStorage,
};

async fn setup_test_db() -> sea_orm::DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    let builder = db.get_database_backend();
    let schema = Schema::new(builder);
    let stmt = schema.create_table_from_entity(reference::Entity);
    db.execute(builder.build(&stmt)).await.unwrap();
    db
}

#[tokio::test]
async fn test_create_task_tool() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let db_conn = Arc::new(setup_test_db().await);
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
        db_conn,
    ));
    let server = LibraMcpServer::new(Some(history_manager), Some(storage));

    let params = CreateTaskParams {
        title: "Test Task".to_string(),
        intent_id: None,
        description: Some("Description".to_string()),
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
        requested_by_kind: None,
        requested_by_id: None,
        dependencies: None,
        parent_task_id: None,
        origin_step_id: None,
        status: None,
        reason: None,
        tags: None,
        external_ids: None,
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
