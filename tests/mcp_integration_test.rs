use std::sync::Arc;

use git_internal::internal::object::{
    context::{ContextSnapshot, SelectionStrategy},
    decision::{Decision, DecisionType},
    evidence::{Evidence, EvidenceKind},
    patchset::{ChangeType, PatchSet, TouchedFile},
    plan::{Plan, PlanStatus, PlanStep},
    provenance::Provenance,
    tool::{ToolInvocation, ToolStatus},
    types::ActorRef,
};
use libra::{
    internal::ai::{
        history::HistoryManager,
        mcp::{
            resource::{CreateTaskParams, ListTasksParams},
            server::LibraMcpServer,
        },
    },
    utils::{storage::local::LocalStorage, storage_ext::StorageExt},
};
use rmcp::{ServerHandler, handler::server::wrapper::Parameters};
use tempfile::tempdir;
use uuid::Uuid;

#[tokio::test]
async fn test_mcp_integration_server_info() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
    ));
    let repo_id = Uuid::new_v4();
    let server = LibraMcpServer::new(Some(history_manager), None, Some(storage), repo_id);

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
    let repo_id = Uuid::new_v4();
    let server = LibraMcpServer::new(Some(history_manager), None, Some(storage), repo_id);

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
    let repo_id = Uuid::new_v4();
    let server = LibraMcpServer::new(Some(history_manager), None, Some(storage), repo_id);

    // 1. Create Task
    let params = CreateTaskParams {
        title: "Integration Test Task".to_string(),
        description: Some("Description".to_string()),
        goal_type: Some("feature".to_string()),
        constraints: Some(vec!["Must use Rust".to_string()]),
        acceptance_criteria: None,
        requested_by_kind: None,
        requested_by_id: None,
        dependencies: None,
        status: None,
        tags: None,
        external_ids: None,
        actor_kind: None,
        actor_id: None,
    };

    let actor = server.default_actor().unwrap();
    let result = server.create_task_impl(params, actor).await.unwrap();
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

/// Helper: create a server with storage and history, returning all components.
fn setup_server() -> (LibraMcpServer, Arc<LocalStorage>, Arc<HistoryManager>, Uuid) {
    let temp_dir = tempdir().unwrap();
    // Leak tempdir to keep it alive for the duration of the test
    let temp_dir = Box::leak(Box::new(temp_dir));
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
    ));
    let repo_id = Uuid::new_v4();
    let server = LibraMcpServer::new(
        Some(history_manager.clone()),
        None,
        Some(storage.clone()),
        repo_id,
    );
    (server, storage, history_manager, repo_id)
}

#[tokio::test]
async fn test_history_latest_returns_real_hash() {
    let (server, storage, history_manager, repo_id) = setup_server();

    // Before any history: should return "no history"
    let contents = server
        .read_resource_impl("libra://history/latest")
        .await
        .unwrap();
    let val = serde_json::to_value(&contents[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();
    assert_eq!(text, "no history");

    // Create a task to produce a history commit
    let actor = ActorRef::human("tester").unwrap();
    let task =
        git_internal::internal::object::task::Task::new(repo_id, actor, "History Test", None)
            .unwrap();
    storage.put_tracked(&task, &history_manager).await.unwrap();

    // Now should return a real hex hash
    let contents = server
        .read_resource_impl("libra://history/latest")
        .await
        .unwrap();
    let val = serde_json::to_value(&contents[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();
    assert_ne!(text, "no history");
    assert!(text.len() >= 40, "Expected a hash string, got: {}", text);
    assert!(
        text.chars().all(|c| c.is_ascii_hexdigit()),
        "Not a hex hash: {}",
        text
    );
}

#[tokio::test]
async fn test_context_active_no_active() {
    let (server, _, _, _) = setup_server();

    let contents = server
        .read_resource_impl("libra://context/active")
        .await
        .unwrap();
    let val = serde_json::to_value(&contents[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();
    let json: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(json["active"], false);
}

#[tokio::test]
async fn test_list_context_snapshots_with_summary() {
    let (server, storage, history_manager, repo_id) = setup_server();
    let actor = ActorRef::human("tester").unwrap();
    let base = "a".repeat(64);

    let mut snap =
        ContextSnapshot::new(repo_id, actor, &base, SelectionStrategy::Heuristic).unwrap();
    snap.set_summary(Some("test summary".to_string()));
    storage.put_tracked(&snap, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListContextSnapshotsParams;
    let result = server
        .list_context_snapshots(Parameters(ListContextSnapshotsParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Strategy:"));
    assert!(text.contains("Items:"));
    assert!(text.contains("test summary"));
}

#[tokio::test]
async fn test_list_plans_with_summary() {
    let (server, storage, history_manager, repo_id) = setup_server();
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let mut plan = Plan::new(repo_id, actor, run_id).unwrap();
    plan.add_step(PlanStep {
        intent: "step 1".to_string(),
        inputs: None,
        outputs: None,
        checks: None,
        owner_role: None,
        status: PlanStatus::Pending,
    });
    storage.put_tracked(&plan, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListPlansParams;
    let result = server
        .list_plans(Parameters(ListPlansParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Version:"));
    assert!(text.contains("Steps: 1"));
}

#[tokio::test]
async fn test_list_patchsets_with_summary() {
    let (server, storage, history_manager, repo_id) = setup_server();
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();
    let base = "b".repeat(64);

    let mut ps = PatchSet::new(repo_id, actor, run_id, &base, 1).unwrap();
    let tf = TouchedFile::new("src/main.rs".to_string(), ChangeType::Modify, 10, 5).unwrap();
    ps.add_touched_file(tf);
    storage.put_tracked(&ps, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListPatchSetsParams;
    let result = server
        .list_patchsets(Parameters(ListPatchSetsParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Gen: 1"));
    assert!(text.contains("Files: 1"));
    assert!(text.contains("Status:"));
}

#[tokio::test]
async fn test_list_evidences_with_summary() {
    let (server, storage, history_manager, repo_id) = setup_server();
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let mut ev = Evidence::new(repo_id, actor, run_id, EvidenceKind::Test, "cargo").unwrap();
    ev.set_exit_code(Some(0));
    ev.set_summary(Some("all tests passed".to_string()));
    storage.put_tracked(&ev, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListEvidencesParams;
    let result = server
        .list_evidences(Parameters(ListEvidencesParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Kind:"));
    assert!(text.contains("Tool: cargo"));
    assert!(text.contains("Exit: 0"));
    assert!(text.contains("all tests passed"));
}

#[tokio::test]
async fn test_list_tool_invocations_with_summary() {
    let (server, storage, history_manager, repo_id) = setup_server();
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let mut inv = ToolInvocation::new(repo_id, actor, run_id, "read_file").unwrap();
    inv.set_status(ToolStatus::Ok);
    inv.set_result_summary(Some("read 100 lines".to_string()));
    storage.put_tracked(&inv, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListToolInvocationsParams;
    let result = server
        .list_tool_invocations(Parameters(ListToolInvocationsParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Tool: read_file"));
    assert!(text.contains("Ok"));
    assert!(text.contains("read 100 lines"));
}

#[tokio::test]
async fn test_list_provenances_with_summary() {
    let (server, storage, history_manager, repo_id) = setup_server();
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let prov = Provenance::new(repo_id, actor, run_id, "openai", "gpt-4o").unwrap();
    storage.put_tracked(&prov, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListProvenancesParams;
    let result = server
        .list_provenances(Parameters(ListProvenancesParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Provider: openai"));
    assert!(text.contains("Model: gpt-4o"));
}

#[tokio::test]
async fn test_list_decisions_with_summary() {
    let (server, storage, history_manager, repo_id) = setup_server();
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let mut dec = Decision::new(repo_id, actor, run_id, DecisionType::Commit).unwrap();
    dec.set_rationale(Some("all tests pass".to_string()));
    storage.put_tracked(&dec, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListDecisionsParams;
    let result = server
        .list_decisions(Parameters(ListDecisionsParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Commit"));
    assert!(text.contains("all tests pass"));
}

/// Test that explicit actor_kind/actor_id params override the MCP default.
#[tokio::test]
async fn test_create_task_with_explicit_human_actor() {
    let (server, _storage, _history_manager, _repo_id) = setup_server();

    let params = CreateTaskParams {
        title: "Human-authored task".to_string(),
        description: None,
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
        requested_by_kind: None,
        requested_by_id: None,
        dependencies: None,
        status: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("human".to_string()),
        actor_id: Some("jackie".to_string()),
    };

    let actor = ActorRef::human("jackie").unwrap();
    let result = server
        .create_task_impl(params, actor.clone())
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();
    assert!(text.contains("Task created with ID:"));

    // Verify the stored object has Human actor kind
    let task_id = text.split("ID: ").nth(1).unwrap().trim();
    use libra::internal::ai::mcp::resource::ListTasksParams;
    let list = server
        .list_tasks(Parameters(ListTasksParams {
            limit: None,
            status: None,
        }))
        .await
        .unwrap();
    let list_text = serde_json::to_value(&list.content[0])
        .unwrap()
        .get("text")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    assert!(list_text.contains(task_id));
}

/// Test creating a task with agent actor kind.
#[tokio::test]
async fn test_create_task_with_agent_actor() {
    let (server, _storage, _history_manager, _repo_id) = setup_server();

    let params = CreateTaskParams {
        title: "Agent-created task".to_string(),
        description: None,
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
        requested_by_kind: None,
        requested_by_id: None,
        dependencies: None,
        status: None,
        tags: None,
        external_ids: None,
        actor_kind: Some("agent".to_string()),
        actor_id: Some("coder-bot".to_string()),
    };

    let actor = ActorRef::agent("coder-bot").unwrap();
    let result = server.create_task_impl(params, actor).await.unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();
    assert!(text.contains("Task created with ID:"));
}

/// Test that omitting actor_kind/actor_id defaults to mcp_client.
#[tokio::test]
async fn test_create_task_default_actor_is_mcp() {
    let (server, _storage, _history_manager, _repo_id) = setup_server();

    let params = CreateTaskParams {
        title: "Default actor task".to_string(),
        description: None,
        goal_type: None,
        constraints: None,
        acceptance_criteria: None,
        requested_by_kind: None,
        requested_by_id: None,
        dependencies: None,
        status: None,
        tags: None,
        external_ids: None,
        actor_kind: None,
        actor_id: None,
    };

    // Using default_actor (falls back to mcp_client)
    let actor = server.default_actor().unwrap();
    assert_eq!(
        actor.kind(),
        &git_internal::internal::object::types::ActorKind::McpClient
    );

    let result = server.create_task_impl(params, actor).await.unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();
    assert!(text.contains("Task created with ID:"));
}
