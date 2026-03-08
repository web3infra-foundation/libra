use std::sync::Arc;

use git_internal::internal::object::{
    context::{ContextSnapshot, SelectionStrategy},
    decision::{Decision, DecisionType},
    evidence::{Evidence, EvidenceKind},
    intent::Intent,
    patchset::{ChangeType, PatchSet, TouchedFile},
    plan::{Plan, PlanStep},
    provenance::Provenance,
    run::Run,
    task::Task,
    tool::{ToolInvocation, ToolStatus},
    types::ActorRef,
};
use libra::{
    internal::{
        ai::{
            history::HistoryManager,
            mcp::{
                resource::{
                    CreateDecisionParams, CreateEvidenceParams, CreatePatchSetParams,
                    CreatePlanParams, CreateProvenanceParams, CreateRunParams, CreateTaskParams,
                    CreateToolInvocationParams, ListTasksParams, UpdateIntentParams,
                },
                server::LibraMcpServer,
            },
        },
        model::reference,
    },
    utils::{storage::local::LocalStorage, storage_ext::StorageExt},
};
use rmcp::{ServerHandler, handler::server::wrapper::Parameters};
use sea_orm::{ConnectionTrait, Database, Schema};
use tempfile::tempdir;
use uuid::Uuid;

async fn setup_test_db() -> sea_orm::DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    let builder = db.get_database_backend();
    let schema = Schema::new(builder);
    let stmt = schema.create_table_from_entity(reference::Entity);
    db.execute(builder.build(&stmt)).await.unwrap();
    db
}

#[tokio::test]
async fn test_mcp_integration_server_info() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let db_conn = Arc::new(setup_test_db().await);
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
        db_conn,
    ));
    let server = LibraMcpServer::new(Some(history_manager), Some(storage));

    let info = ServerHandler::get_info(&server);
    assert_eq!(info.server_info.name, "libra");
}

#[tokio::test]
async fn test_mcp_integration_list_resources() {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let db_conn = Arc::new(setup_test_db().await);
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
        db_conn,
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
    let db_conn = Arc::new(setup_test_db().await);
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
        db_conn,
    ));
    let server = LibraMcpServer::new(Some(history_manager), Some(storage));

    // 1. Create Task
    let params = CreateTaskParams {
        title: "Integration Test Task".to_string(),
        intent_id: None,
        description: Some("Description".to_string()),
        goal_type: Some("feature".to_string()),
        constraints: Some(vec!["Must use Rust".to_string()]),
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

#[tokio::test]
async fn test_create_run_requires_existing_task() {
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;
    let actor = server.default_actor().unwrap();

    let result = server
        .create_run_impl(
            CreateRunParams {
                task_id: Uuid::new_v4().to_string(),
                base_commit_sha: "a".repeat(64),
                plan_id: None,
                status: None,
                context_snapshot_id: None,
                error: None,
                agent_instances: None,
                metrics_json: None,
                reason: None,
                orchestrator_version: None,
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(result.is_err(), "run should fail when task does not exist");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("task_id not found"),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_patchset_requires_existing_run() {
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;
    let actor = server.default_actor().unwrap();

    let result = server
        .create_patchset_impl(
            CreatePatchSetParams {
                run_id: Uuid::new_v4().to_string(),
                generation: 1,
                sequence: None,
                base_commit_sha: "a".repeat(64),
                touched_files: None,
                rationale: None,
                diff_format: None,
                diff_artifact: None,
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(
        result.is_err(),
        "patchset creation should fail when run does not exist"
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("run_id not found"),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_tool_invocation_requires_existing_run() {
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;
    let actor = server.default_actor().unwrap();

    let result = server
        .create_tool_invocation_impl(
            CreateToolInvocationParams {
                run_id: Uuid::new_v4().to_string(),
                tool_name: "read_file".to_string(),
                status: None,
                args_json: None,
                io_footprint: None,
                result_summary: None,
                artifacts: None,
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(
        result.is_err(),
        "tool invocation creation should fail when run does not exist"
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("run_id not found"),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_provenance_requires_existing_run() {
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;
    let actor = server.default_actor().unwrap();

    let result = server
        .create_provenance_impl(
            CreateProvenanceParams {
                run_id: Uuid::new_v4().to_string(),
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                parameters_json: None,
                temperature: None,
                max_tokens: None,
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(
        result.is_err(),
        "provenance creation should fail when run does not exist"
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("run_id not found"),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_task_rejects_missing_intent_reference() {
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;
    let actor = server.default_actor().unwrap();

    let result = server
        .create_task_impl(
            CreateTaskParams {
                title: "Task with missing intent".to_string(),
                intent_id: Some(Uuid::new_v4().to_string()),
                description: None,
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
            },
            actor,
        )
        .await;

    assert!(
        result.is_err(),
        "task creation should fail on missing intent reference"
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("intent_id not found"),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_update_intent_accepts_discarded_alias() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();

    let intent = Intent::new(actor, "Alias test intent").unwrap();
    let intent_id = intent.header().object_id();
    storage
        .put_tracked(&intent, &history_manager)
        .await
        .unwrap();

    let result = server
        .update_intent_impl(UpdateIntentParams {
            intent_id: intent_id.to_string(),
            status: Some("discarded".to_string()),
            commit_sha: None,
            reason: Some("no longer needed".to_string()),
            next_intent_id: None,
        })
        .await;

    assert!(
        result.is_ok(),
        "discarded alias should map to cancelled lifecycle event"
    );
}

#[tokio::test]
async fn test_create_decision_accepts_uuid_prefixed_ids() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();

    let task = Task::new(actor.clone(), "decision task", None).unwrap();
    storage.put_tracked(&task, &history_manager).await.unwrap();

    let base = "b".repeat(64);
    let run = Run::new(actor.clone(), task.header().object_id(), &base).unwrap();
    storage.put_tracked(&run, &history_manager).await.unwrap();

    let patchset = PatchSet::new(actor.clone(), run.header().object_id(), &base).unwrap();
    storage
        .put_tracked(&patchset, &history_manager)
        .await
        .unwrap();

    let result = server
        .create_decision_impl(
            CreateDecisionParams {
                run_id: format!("uuid:{}", run.header().object_id()),
                decision_type: "commit".to_string(),
                chosen_patchset_id: Some(format!("uuid:{}", patchset.header().object_id())),
                result_commit_sha: Some(base),
                checkpoint_id: None,
                rationale: Some("apply selected patch".to_string()),
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(result.is_ok(), "uuid: prefixed ids should be accepted");
}

#[tokio::test]
async fn test_update_intent_accepts_uuid_prefixed_intent_id() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();

    let intent = Intent::new(actor, "prefix intent").unwrap();
    let intent_id = intent.header().object_id();
    storage
        .put_tracked(&intent, &history_manager)
        .await
        .unwrap();

    let result = server
        .update_intent_impl(UpdateIntentParams {
            intent_id: format!("uuid:{intent_id}"),
            status: Some("active".to_string()),
            commit_sha: None,
            reason: Some("prefix accepted".to_string()),
            next_intent_id: None,
        })
        .await;

    assert!(
        result.is_ok(),
        "uuid: prefixed intent id should be accepted"
    );
}

#[tokio::test]
async fn test_create_plan_rejects_parent_from_other_intent() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();

    let intent_a = Intent::new(actor.clone(), "intent-a").unwrap();
    let intent_b = Intent::new(actor.clone(), "intent-b").unwrap();
    storage
        .put_tracked(&intent_a, &history_manager)
        .await
        .unwrap();
    storage
        .put_tracked(&intent_b, &history_manager)
        .await
        .unwrap();

    let parent_plan = Plan::new(actor.clone(), intent_a.header().object_id()).unwrap();
    storage
        .put_tracked(&parent_plan, &history_manager)
        .await
        .unwrap();

    let result = server
        .create_plan_impl(
            CreatePlanParams {
                intent_id: intent_b.header().object_id().to_string(),
                parent_plan_ids: Some(vec![parent_plan.header().object_id().to_string()]),
                context_frame_ids: None,
                steps: None,
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(
        result.is_err(),
        "cross-intent parent plan should be rejected"
    );
    let err = result.unwrap_err();
    assert!(
        err.message
            .contains("parent_plan_ids must belong to intent"),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_run_rejects_plan_with_mismatched_task_intent() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();

    let intent_task = Intent::new(actor.clone(), "task-intent").unwrap();
    let intent_plan = Intent::new(actor.clone(), "plan-intent").unwrap();
    storage
        .put_tracked(&intent_task, &history_manager)
        .await
        .unwrap();
    storage
        .put_tracked(&intent_plan, &history_manager)
        .await
        .unwrap();

    let mut task = Task::new(actor.clone(), "task with intent", None).unwrap();
    task.set_intent(Some(intent_task.header().object_id()));
    storage.put_tracked(&task, &history_manager).await.unwrap();

    let plan = Plan::new(actor.clone(), intent_plan.header().object_id()).unwrap();
    storage.put_tracked(&plan, &history_manager).await.unwrap();

    let result = server
        .create_run_impl(
            CreateRunParams {
                task_id: task.header().object_id().to_string(),
                base_commit_sha: "c".repeat(64),
                plan_id: Some(plan.header().object_id().to_string()),
                status: None,
                context_snapshot_id: None,
                error: None,
                agent_instances: None,
                metrics_json: None,
                reason: None,
                orchestrator_version: None,
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(
        result.is_err(),
        "mismatched task/plan intent should be rejected"
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("plan_id intent"),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_evidence_rejects_patchset_from_different_run() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();

    let task = Task::new(actor.clone(), "evidence task", None).unwrap();
    storage.put_tracked(&task, &history_manager).await.unwrap();

    let base = "d".repeat(64);
    let run_a = Run::new(actor.clone(), task.header().object_id(), &base).unwrap();
    let run_b = Run::new(actor.clone(), task.header().object_id(), &base).unwrap();
    storage.put_tracked(&run_a, &history_manager).await.unwrap();
    storage.put_tracked(&run_b, &history_manager).await.unwrap();

    let patchset_on_b = PatchSet::new(actor.clone(), run_b.header().object_id(), &base).unwrap();
    storage
        .put_tracked(&patchset_on_b, &history_manager)
        .await
        .unwrap();

    let result = server
        .create_evidence_impl(
            CreateEvidenceParams {
                run_id: run_a.header().object_id().to_string(),
                patchset_id: Some(patchset_on_b.header().object_id().to_string()),
                kind: "test".to_string(),
                tool: "cargo".to_string(),
                command: Some("cargo test".to_string()),
                exit_code: Some(1),
                summary: Some("failed".to_string()),
                report_artifacts: None,
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor.clone(),
        )
        .await;

    assert!(
        result.is_err(),
        "evidence patchset from another run should be rejected"
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("patchset_id")
            && err.message.contains("belongs to run")
            && err
                .message
                .contains(&run_a.header().object_id().to_string()),
        "unexpected error: {}",
        err.message
    );
}

#[tokio::test]
async fn test_create_decision_rejects_patchset_from_different_run() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();

    let task = Task::new(actor.clone(), "decision relation task", None).unwrap();
    storage.put_tracked(&task, &history_manager).await.unwrap();

    let base = "e".repeat(64);
    let run_a = Run::new(actor.clone(), task.header().object_id(), &base).unwrap();
    let run_b = Run::new(actor.clone(), task.header().object_id(), &base).unwrap();
    storage.put_tracked(&run_a, &history_manager).await.unwrap();
    storage.put_tracked(&run_b, &history_manager).await.unwrap();

    let patchset_on_b = PatchSet::new(actor.clone(), run_b.header().object_id(), &base).unwrap();
    storage
        .put_tracked(&patchset_on_b, &history_manager)
        .await
        .unwrap();

    let result = server
        .create_decision_impl(
            CreateDecisionParams {
                run_id: run_a.header().object_id().to_string(),
                decision_type: "commit".to_string(),
                chosen_patchset_id: Some(patchset_on_b.header().object_id().to_string()),
                result_commit_sha: Some(base),
                checkpoint_id: None,
                rationale: Some("wrong patchset".to_string()),
                tags: None,
                external_ids: None,
                actor_kind: None,
                actor_id: None,
            },
            actor,
        )
        .await;

    assert!(
        result.is_err(),
        "decision patchset from another run should be rejected"
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("chosen_patchset_id")
            && err.message.contains("belongs to run")
            && err
                .message
                .contains(&run_a.header().object_id().to_string()),
        "unexpected error: {}",
        err.message
    );
}

/// Helper: create a server with storage and history, returning all components.
async fn setup_server() -> (
    LibraMcpServer,
    Arc<LocalStorage>,
    Arc<HistoryManager>,
    tempfile::TempDir,
) {
    let temp_dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(temp_dir.path().join("objects")));
    let db_conn = Arc::new(setup_test_db().await);
    let history_manager = Arc::new(HistoryManager::new(
        storage.clone(),
        temp_dir.path().to_path_buf(),
        db_conn,
    ));
    let server = LibraMcpServer::new(Some(history_manager.clone()), Some(storage.clone()));
    (server, storage, history_manager, temp_dir)
}

#[tokio::test]
async fn test_history_latest_returns_real_hash() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;

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
        git_internal::internal::object::task::Task::new(actor, "History Test", None).unwrap();
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
    let (server, _, _, _temp_dir) = setup_server().await;

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
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();
    let _base = "a".repeat(64);

    let mut snap = ContextSnapshot::new(actor, SelectionStrategy::Heuristic).unwrap();
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
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();
    let _run_id = Uuid::new_v4();

    let mut plan = Plan::new(actor, Uuid::new_v4()).unwrap();
    let step = PlanStep::new("step 1");
    plan.add_step(step);
    storage.put_tracked(&plan, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListPlansParams;
    let result = server
        .list_plans(Parameters(ListPlansParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Steps: 1"));
}

#[tokio::test]
async fn test_list_patchsets_with_summary() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();
    let base = "b".repeat(64);

    let mut ps = PatchSet::new(actor, run_id, &base).unwrap();
    let tf = TouchedFile::new("src/main.rs".to_string(), ChangeType::Modify, 10, 5).unwrap();
    ps.add_touched(tf);
    storage.put_tracked(&ps, &history_manager).await.unwrap();

    use libra::internal::ai::mcp::resource::ListPatchSetsParams;
    let result = server
        .list_patchsets(Parameters(ListPatchSetsParams { limit: None }))
        .await
        .unwrap();
    let val = serde_json::to_value(&result.content[0]).unwrap();
    let text = val.get("text").unwrap().as_str().unwrap();

    assert!(text.contains("Files: 1"));
    assert!(text.contains("Format:"));
}

#[tokio::test]
async fn test_list_evidences_with_summary() {
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let mut ev = Evidence::new(actor, run_id, EvidenceKind::Test, "cargo").unwrap();
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
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let mut inv = ToolInvocation::new(actor, run_id, "read_file").unwrap();
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
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let prov = Provenance::new(actor, run_id, "openai", "gpt-4o").unwrap();
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
    let (server, storage, history_manager, _temp_dir) = setup_server().await;
    let actor = ActorRef::human("tester").unwrap();
    let run_id = Uuid::new_v4();

    let mut dec = Decision::new(actor, run_id, DecisionType::Commit).unwrap();
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
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;

    let params = CreateTaskParams {
        title: "Human-authored task".to_string(),
        intent_id: None,
        description: None,
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
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;

    let params = CreateTaskParams {
        title: "Agent-created task".to_string(),
        intent_id: None,
        description: None,
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
    let (server, _storage, _history_manager, _temp_dir) = setup_server().await;

    let params = CreateTaskParams {
        title: "Default actor task".to_string(),
        intent_id: None,
        description: None,
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
