use std::{str::FromStr, sync::Arc};

use git_internal::{
    hash::ObjectHash,
    internal::object::{
        plan::Plan,
        run::Run,
        task::{GoalType, Task},
        types::{ActorRef, ObjectType},
    },
};
use libra::{
    command::commit::CommitArgs,
    internal::{ai::history::HistoryManager, head::Head},
    utils::{
        storage::{Storage, local::LocalStorage, remote::RemoteStorage},
        storage_ext::StorageExt,
        test,
    },
};
use tempfile::tempdir;
use uuid::Uuid;

/// Integration test for the full AI storage flow using LocalStorage
#[tokio::test]
async fn test_ai_flow_local() {
    // 1. Setup Storage and Repo Environment
    let dir = tempdir().unwrap();
    // Change directory so try_get_storage_path finds the repo
    let _guard = test::ChangeDirGuard::new(dir.path());

    test::setup_with_new_libra_in(dir.path()).await;

    let libra_dir = dir.path().join(".libra");
    let objects_dir = libra_dir.join("objects");

    let storage = Arc::new(LocalStorage::new(objects_dir));
    let history_manager = HistoryManager::new(storage.clone(), libra_dir.clone());

    // 2. User creates a Task
    let repo_id = Uuid::new_v4();
    let actor = ActorRef::human("jackie").unwrap();
    let mut task = Task::new(
        repo_id,
        actor.clone(),
        "Refactor Storage",
        Some(GoalType::Refactor),
    )
    .unwrap();
    task.add_constraint("Must use StorageExt");

    // Use put_tracked to ensure History Log is updated (Orphan Branch)
    let task_hash = storage.put_tracked(&task, &history_manager).await.unwrap();
    println!("Stored Task: {}", task_hash);

    // Verify History Log Creation
    // The history ref should exist and point to a commit
    let history_ref_path = libra_dir.join("refs/libra/history");
    assert!(
        history_ref_path.exists(),
        "History ref file should be created at {:?}",
        history_ref_path
    );

    libra::command::commit::execute(CommitArgs {
        message: Some("initial commit".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: true,
        ..Default::default()
    })
    .await;

    let head_commit = Head::current_commit().await.unwrap().to_string();
    let base_commit_sha = libra::internal::ai::util::normalize_commit_anchor(&head_commit).unwrap();

    let snapshot = git_internal::internal::object::context::ContextSnapshot::new(
        repo_id,
        actor.clone(),
        &base_commit_sha,
        git_internal::internal::object::context::SelectionStrategy::Heuristic,
    )
    .unwrap();

    let snapshot_hash = storage
        .put_tracked(&snapshot, &history_manager)
        .await
        .unwrap();
    println!("Stored Snapshot: {}", snapshot_hash);

    // 2.6. User creates a Run
    let mut run = Run::new(
        repo_id,
        actor.clone(),
        task.header().object_id(),
        &base_commit_sha,
    )
    .unwrap();
    run.set_context_snapshot_id(Some(snapshot.header().object_id()));

    let run_hash = storage.put_tracked(&run, &history_manager).await.unwrap();
    println!("Stored Run: {}", run_hash);

    // 2.7. User creates a Plan
    let plan = Plan::new(repo_id, actor.clone(), run.header().object_id()).unwrap();
    let plan_hash = storage.put_tracked(&plan, &history_manager).await.unwrap();
    println!("Stored Plan: {}", plan_hash);

    // Verify Run Retrieval
    let loaded_run: Run = storage.get_json(&run_hash).await.unwrap();
    assert_eq!(run.header().object_id(), loaded_run.header().object_id());

    // Verify Plan Retrieval
    let loaded_plan: Plan = storage.get_json(&plan_hash).await.unwrap();
    assert_eq!(plan.header().object_id(), loaded_plan.header().object_id());

    // 3. Verify Task Retrieval
    let loaded_task: Task = storage.get_json(&task_hash).await.unwrap();
    assert_eq!(task.title(), loaded_task.title());
    assert_eq!(task.constraints(), loaded_task.constraints());

    // 4. Create an Artifact (simulating a Plan or Patch)
    let patch_content = b"diff --git a/src/main.rs b/src/main.rs\n...";
    let artifact = storage.put_artifact(patch_content).await.unwrap();
    println!("Stored Artifact: {}", artifact.key());

    assert_eq!(artifact.store(), "libra");

    // 5. Verify Artifact Retrieval (via StorageExt or underlying Storage)
    let artifact_hash = ObjectHash::from_str(artifact.key()).unwrap();
    let (data, obj_type) = storage.get(&artifact_hash).await.unwrap();
    assert_eq!(obj_type, ObjectType::Blob);
    assert_eq!(data, patch_content);

    // 6. Verify Normal Blob Storage works alongside
    let blob_content = b"Standard Blob Content";
    let blob_hash = ObjectHash::from_type_and_data(ObjectType::Blob, blob_content);
    storage
        .put(&blob_hash, blob_content, ObjectType::Blob)
        .await
        .unwrap();

    let (loaded_blob, _) = storage.get(&blob_hash).await.unwrap();
    assert_eq!(loaded_blob, blob_content);
}

/// Integration test for AI storage flow using Cloudflare R2 (S3-compatible)
///
/// To run this test manually:
/// 1. Set the following environment variables:
///    - R2_ENDPOINT: Your R2 endpoint URL
///    - R2_ACCESS_KEY: Your Access Key ID
///    - R2_SECRET_KEY: Your Secret Access Key
///    - R2_BUCKET: Target bucket name
///    - R2_REGION: Region (usually "auto" for R2)
/// 2. Run: `cargo test --test ai_storage_flow_test -- --ignored`
///
/// This test verifies that:
/// - Objects can be stored and retrieved from R2
/// - Artifacts are correctly stored in R2
/// - Connectivity to the remote storage provider works as expected
#[tokio::test]
#[ignore]
async fn test_ai_flow_r2() {
    // 1. Load Config from Env
    let endpoint = std::env::var("R2_ENDPOINT").expect("R2_ENDPOINT not set");
    let access_key = std::env::var("R2_ACCESS_KEY").expect("R2_ACCESS_KEY not set");
    let secret_key = std::env::var("R2_SECRET_KEY").expect("R2_SECRET_KEY not set");
    let bucket = std::env::var("R2_BUCKET").expect("R2_BUCKET not set");
    let region = std::env::var("R2_REGION").unwrap_or_else(|_| "auto".to_string());

    // 2. Setup Remote Storage (Using object_store directly to avoid coupling RemoteStorage to specific backends)
    let s3 = object_store::aws::AmazonS3Builder::new()
        .with_bucket_name(&bucket)
        .with_region(&region)
        .with_endpoint(&endpoint)
        .with_access_key_id(&access_key)
        .with_secret_access_key(&secret_key)
        .with_virtual_hosted_style_request(false)
        .build()
        .expect("Failed to build S3 client");

    let storage = Arc::new(RemoteStorage::new(Arc::new(s3)));

    // 3. User creates a Task
    let repo_id = Uuid::new_v4();
    let actor = ActorRef::human("jackie-r2").unwrap();
    let task = Task::new(repo_id, actor, "Test R2 Storage", Some(GoalType::Chore)).unwrap();

    let task_hash = storage.put_json(&task).await.unwrap();
    println!("Stored Task to R2: {}", task_hash);

    // 4. Verify Task Retrieval from R2
    let loaded_task: Task = storage.get_json(&task_hash).await.unwrap();
    assert_eq!(task.title(), loaded_task.title());

    // 5. Create Artifact
    let artifact_content = b"Cloud Content";
    let artifact = storage.put_artifact(artifact_content).await.unwrap();
    println!("Stored Artifact to R2: {}", artifact.key());

    // 6. Verify Artifact Retrieval
    let artifact_hash = ObjectHash::from_str(artifact.key()).unwrap();
    let (data, _) = storage.get(&artifact_hash).await.unwrap();
    assert_eq!(data, artifact_content);
}
