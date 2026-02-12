#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::{handler::server::wrapper::Parameters, model::Content};
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
        // Setup
        let temp_dir = tempdir().unwrap();
        let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
        let history_manager = Arc::new(HistoryManager::new(
            storage.clone(),
            temp_dir.path().to_path_buf(),
        ));
        let server = LibraMcpServer::new(Some(history_manager), Some(storage));

        // Test create_task
        let params = CreateTaskParams {
            title: "Test Task".to_string(),
            description: Some("Description".to_string()),
        };

        let result = server.create_task(Parameters(params)).await;
        assert!(result.is_ok());

        let call_result = result.unwrap();
        assert!(!call_result.content.is_empty());
        let content = &call_result.content[0];
        // Check if content matches text type manually since enum might be hidden or structure is different
        // rmcp::model::Content is likely a type alias or struct wrapping RawContent
        // Let's debug print it first or assume structure.
        // Looking at error: `Annotated<RawContent>` does not have `Text` variant.
        // It seems `Content` is `Annotated<RawContent>`.
        // And `RawContent` might be an enum or struct.
        // Let's try to match on the inner raw content if possible, or use accessors.
        // But `rmcp` 0.13 usually has `RawContent` as an enum with `TextResource` etc?
        // Wait, error says `associated item not found in Annotated<RawContent>`.
        // So `Content` IS `Annotated<RawContent>`.
        // We need to check `content.raw`.

        use rmcp::model::RawContent;
        match &content.raw {
            RawContent::Text(text_content) => {
                assert!(text_content.text.contains("Task created with ID"));
            }
            _ => panic!("Expected text content"),
        }
    }
}
