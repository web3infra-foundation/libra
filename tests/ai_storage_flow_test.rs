use std::{str::FromStr, sync::Arc};

use git_internal::{
    hash::ObjectHash,
    internal::object::{
        task::{GoalType, Task},
        types::{ActorRef, ObjectType},
    },
};
use libra::utils::{
    storage::{Storage, local::LocalStorage, remote::RemoteStorage},
    storage_ext::StorageExt,
};
use tempfile::tempdir;
use uuid::Uuid;

/// Integration test for the full AI storage flow using LocalStorage
#[tokio::test]
async fn test_ai_flow_local() {
    // 1. Setup Storage
    let dir = tempdir().unwrap();
    let storage = Arc::new(LocalStorage::new(dir.path().to_path_buf()));

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

    let task_hash = storage.put_json(&task).await.unwrap();
    println!("Stored Task: {}", task_hash);

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
